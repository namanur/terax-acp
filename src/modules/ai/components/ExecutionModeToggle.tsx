import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { type ExecutionMode } from "@/modules/settings/store";

interface ExecutionModeToggleProps {
  mode: ExecutionMode;
  onModeChange: (mode: ExecutionMode) => void;
}

export function ExecutionModeToggle({
  mode,
  onModeChange,
}: ExecutionModeToggleProps) {
  return (
    <div className="flex flex-col gap-3">
      <span className="text-[11px] font-medium tracking-tight text-muted-foreground">
        Execution Mode
      </span>
      <RadioGroup
        value={mode}
        onValueChange={(v) => onModeChange(v as ExecutionMode)}
        className="flex flex-col gap-2.5 rounded-lg border border-border/60 bg-card/60 px-3 py-2.5"
      >
        <div className="flex items-center space-x-2">
          <RadioGroupItem value="api_provider" id="r1" />
          <Label htmlFor="r1" className="text-[12px] font-medium">
            API Provider
          </Label>
        </div>
        <div className="flex items-center space-x-2">
          <RadioGroupItem value="acp_agent" id="r2" />
          <Label htmlFor="r2" className="text-[12px] font-medium">
            ACP Agent
          </Label>
        </div>
      </RadioGroup>
    </div>
  );
}
