import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import {
  BUILTIN_ACP_AGENTS,
  newAcpAgentId,
  type AcpAgentServer,
} from "@/modules/ai/lib/acpServers";
import { useAcpAgentsStore } from "@/modules/ai/store/acpServersStore";
import { usePreferencesStore } from "@/modules/settings/preferences";
import { setAcpAgentId } from "@/modules/settings/store";
import { invoke } from "@tauri-apps/api/core";
import {
  Add01Icon,
  CheckmarkCircle02Icon,
  Edit02Icon,
  RefreshIcon,
  Cancel01Icon,
  Delete02Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useEffect, useMemo, useState } from "react";

type ProbeSummary = {
  command: string;
  found: boolean;
  resolved_path: string | null;
  version: string | null;
  capabilities: {
    detected: boolean;
  };
};

export function AcpAgentsBlock({ active }: { active: boolean }) {
  const selectedId = usePreferencesStore((s) => s.acpAgentId);
  const hydrate = useAcpAgentsStore((s) => s.hydrate);
  const customAgents = useAcpAgentsStore((s) => s.customAgents);
  const upsert = useAcpAgentsStore((s) => s.upsert);
  const remove = useAcpAgentsStore((s) => s.remove);
  const [editing, setEditing] = useState<AcpAgentServer | null>(null);
  const [probes, setProbes] = useState<Record<string, ProbeSummary | null>>({});
  const [probing, setProbing] = useState(false);

  const agents = useMemo(
    () => [...BUILTIN_ACP_AGENTS, ...customAgents],
    [customAgents],
  );

  useEffect(() => {
    void hydrate();
  }, [hydrate]);

  const runProbe = async (agent: AcpAgentServer) => {
    try {
      const result = await invoke<ProbeSummary>("agent_probe", {
        command: agent.command,
      });
      setProbes((prev) => ({ ...prev, [agent.id]: result }));
    } catch (error) {
      console.error(`Failed to probe ACP agent ${agent.command}`, error);
      setProbes((prev) => ({ ...prev, [agent.id]: null }));
    }
  };

  const refreshAll = async () => {
    setProbing(true);
    try {
      await Promise.all(agents.map((agent) => runProbe(agent)));
    } finally {
      setProbing(false);
    }
  };

  useEffect(() => {
    if (agents.length === 0) return;
    void refreshAll();
  }, [agents]);

  const clearIfSelected = async (id: string) => {
    if (selectedId === id) {
      await setAcpAgentId(null);
    }
  };

  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-start justify-between gap-3">
        <div className="flex flex-col">
          <span className="text-[12px] font-medium">ACP agents</span>
          <span className="text-[10.5px] text-muted-foreground">
            Add custom ACP agent commands the same way editors like Zed expose
            external agent servers: command, args, env, then select one to use.
          </span>
        </div>
        <div className="flex items-center gap-1.5">
          <Button
            size="sm"
            variant="outline"
            className="h-7 gap-1.5 px-2 text-[11px]"
            onClick={() => void refreshAll()}
            disabled={probing}
          >
            <HugeiconsIcon icon={RefreshIcon} size={12} strokeWidth={1.75} />
            Scan
          </Button>
          <Button
            size="sm"
            variant="outline"
            className="h-7 gap-1.5 px-2 text-[11px]"
            onClick={() =>
              setEditing({
                id: newAcpAgentId(),
                name: "Custom ACP agent",
                description: "",
                command: "",
                args: [],
                env: {},
                builtIn: false,
              })
            }
          >
            <HugeiconsIcon icon={Add01Icon} size={12} strokeWidth={1.75} />
            Add custom
          </Button>
        </div>
      </div>

      {!active ? (
        <div className="rounded-lg border border-dashed border-border/60 bg-card/30 px-3 py-2 text-[11px] text-muted-foreground">
          Configure ACP agents here first. These settings are used when you
          switch execution mode to <span className="font-mono">acp_agent</span>.
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        {agents.map((agent) => {
          const selected = selectedId === agent.id;
          const probe = probes[agent.id];
          const found = probe?.found ?? false;
          return (
            <div
              key={agent.id}
              className={cn(
                "flex flex-col gap-2 rounded-lg border bg-card/60 px-3 py-3",
                selected
                  ? "border-foreground/30 ring-1 ring-foreground/10"
                  : "border-border/60",
              )}
            >
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="truncate text-[12.5px] font-medium">
                      {agent.name}
                    </span>
                    {agent.builtIn ? (
                      <span className="rounded bg-muted/50 px-1 py-0.5 text-[9px] uppercase tracking-wide text-muted-foreground">
                        Built-in
                      </span>
                    ) : null}
                    {selected ? (
                      <span className="rounded bg-foreground/8 px-1 py-0.5 text-[9px] uppercase tracking-wide text-foreground/80">
                        Selected
                      </span>
                    ) : null}
                  </div>
                  {agent.description ? (
                    <p className="mt-0.5 text-[10.5px] text-muted-foreground">
                      {agent.description}
                    </p>
                  ) : null}
                </div>
                <div
                  className={cn(
                    "mt-0.5 size-2.5 rounded-full",
                    found ? "bg-emerald-500" : "bg-muted-foreground/35",
                  )}
                  title={found ? "Detected on PATH" : "Not detected on PATH"}
                />
              </div>

              <div className="space-y-1 text-[11px]">
                <div>
                  <span className="text-muted-foreground">Command: </span>
                  <code className="font-mono">{agent.command || "unset"}</code>
                </div>
                {agent.args.length > 0 ? (
                  <div>
                    <span className="text-muted-foreground">Args: </span>
                    <code className="font-mono">{agent.args.join(" ")}</code>
                  </div>
                ) : null}
                {probe?.version ? (
                  <div>
                    <span className="text-muted-foreground">Version: </span>
                    <span>{probe.version}</span>
                  </div>
                ) : null}
                {probe?.resolved_path ? (
                  <div className="break-all">
                    <span className="text-muted-foreground">Path: </span>
                    <code className="font-mono">{probe.resolved_path}</code>
                  </div>
                ) : (
                  <div className="text-muted-foreground">
                    Not currently found on <span className="font-mono">PATH</span>.
                  </div>
                )}
              </div>

              <div className="flex items-center gap-1.5">
                <Button
                  size="sm"
                  variant={selected ? "secondary" : "outline"}
                  className="h-7 px-2 text-[11px]"
                  onClick={() => void setAcpAgentId(agent.id)}
                >
                  {selected ? (
                    <>
                      <HugeiconsIcon
                        icon={CheckmarkCircle02Icon}
                        size={12}
                        strokeWidth={1.75}
                      />
                      Active
                    </>
                  ) : (
                    "Use this agent"
                  )}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-7 px-2 text-[11px]"
                  onClick={() => void runProbe(agent)}
                >
                  Probe
                </Button>
                <Button
                  size="icon"
                  variant="ghost"
                  className="ml-auto size-7"
                  onClick={() =>
                    setEditing(
                      agent.builtIn
                        ? {
                            ...agent,
                            id: newAcpAgentId(),
                            name: `${agent.name} (Custom)`,
                            builtIn: false,
                          }
                        : agent,
                    )
                  }
                  title={agent.builtIn ? "Customize" : "Edit"}
                >
                  <HugeiconsIcon
                    icon={Edit02Icon}
                    size={12}
                    strokeWidth={1.75}
                  />
                </Button>
                {!agent.builtIn ? (
                  <Button
                    size="icon"
                    variant="ghost"
                    className="size-7 text-muted-foreground hover:text-destructive"
                    onClick={() => {
                      void clearIfSelected(agent.id);
                      remove(agent.id);
                    }}
                    title="Delete"
                  >
                    <HugeiconsIcon
                      icon={Delete02Icon}
                      size={12}
                      strokeWidth={1.75}
                    />
                  </Button>
                ) : null}
              </div>
            </div>
          );
        })}
      </div>

      {active && !selectedId ? (
        <div className="rounded-lg border border-amber-500/25 bg-amber-500/8 px-3 py-2 text-[11px] text-amber-200">
          ACP mode is active, but no ACP agent is selected yet.
        </div>
      ) : null}

      <AcpAgentEditorDialog
        agent={editing}
        onClose={() => setEditing(null)}
        onSave={(agent) => {
          upsert(agent);
          setEditing(null);
        }}
      />
    </section>
  );
}

function AcpAgentEditorDialog({
  agent,
  onClose,
  onSave,
}: {
  agent: AcpAgentServer | null;
  onClose: () => void;
  onSave: (agent: AcpAgentServer) => void;
}) {
  const [draft, setDraft] = useState<AcpAgentServer | null>(agent);
  const [argsText, setArgsText] = useState("");
  const [envText, setEnvText] = useState("");

  useEffect(() => {
    setDraft(agent);
    setArgsText(agent ? agent.args.join("\n") : "");
    setEnvText(agent ? serializeEnv(agent.env) : "");
  }, [agent]);

  const save = () => {
    if (!draft) return;
    onSave({
      ...draft,
      command: draft.command.trim(),
      args: argsText
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean),
      env: parseEnv(envText),
      builtIn: false,
    });
  };

  return (
    <Dialog open={!!draft} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-[560px]">
        <DialogHeader>
          <DialogTitle>
            {draft?.id.startsWith("acp-") ? "Custom ACP agent" : "Edit ACP agent"}
          </DialogTitle>
        </DialogHeader>

        {draft ? (
          <div className="grid gap-3 py-1">
            <div className="grid gap-1.5">
              <span className="text-[11px] font-medium text-muted-foreground">
                Name
              </span>
              <Input
                value={draft.name}
                onChange={(e) => setDraft({ ...draft, name: e.target.value })}
                placeholder="Codex via uvx"
              />
            </div>

            <div className="grid gap-1.5">
              <span className="text-[11px] font-medium text-muted-foreground">
                Description
              </span>
              <Input
                value={draft.description}
                onChange={(e) =>
                  setDraft({ ...draft, description: e.target.value })
                }
                placeholder="What this ACP agent is for"
              />
            </div>

            <div className="grid gap-1.5">
              <span className="text-[11px] font-medium text-muted-foreground">
                Command
              </span>
              <Input
                value={draft.command}
                onChange={(e) =>
                  setDraft({ ...draft, command: e.target.value })
                }
                placeholder="uvx"
              />
            </div>

            <div className="grid gap-1.5">
              <span className="text-[11px] font-medium text-muted-foreground">
                Arguments
              </span>
              <Textarea
                rows={4}
                value={argsText}
                onChange={(e) => setArgsText(e.target.value)}
                placeholder={"One argument per line\nfast-agent-acp@latest\n--model\ngpt-4.1"}
              />
            </div>

            <div className="grid gap-1.5">
              <span className="text-[11px] font-medium text-muted-foreground">
                Environment
              </span>
              <Textarea
                rows={4}
                value={envText}
                onChange={(e) => setEnvText(e.target.value)}
                placeholder={"One KEY=value per line\nOPENAI_API_KEY=...\nACP_CONFIG=/path/to/config.json"}
              />
            </div>
          </div>
        ) : null}

        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            <HugeiconsIcon icon={Cancel01Icon} size={14} strokeWidth={1.75} />
            Cancel
          </Button>
          <Button
            onClick={save}
            disabled={!draft?.name.trim() || !draft.command.trim()}
          >
            <HugeiconsIcon
              icon={CheckmarkCircle02Icon}
              size={14}
              strokeWidth={1.75}
            />
            Save agent
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function parseEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const rawLine of text.split("\n")) {
    const line = rawLine.trim();
    if (!line) continue;
    const idx = line.indexOf("=");
    if (idx === -1) {
      env[line] = "";
      continue;
    }
    const key = line.slice(0, idx).trim();
    const value = line.slice(idx + 1);
    if (key) env[key] = value;
  }
  return env;
}

function serializeEnv(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}
