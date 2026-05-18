import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export interface SessionMode {
  mode_id: string;
  display_name: string;
  description?: string;
  icon?: string;
}

interface ModeSelectorProps {
  modes: SessionMode[];
  current?: string;
  onSelect: (id: string) => void;
  disabled?: boolean;
}

export function ModeSelector({ modes, current, onSelect, disabled }: ModeSelectorProps) {
  if (!modes || modes.length === 0) return null;

  return (
    <Select value={current} onValueChange={onSelect} disabled={disabled}>
      <SelectTrigger className="h-8 w-auto min-w-32 text-xs">
        <SelectValue placeholder="Select mode..." />
      </SelectTrigger>
      <SelectContent>
        {modes.map(mode => (
          <SelectItem key={mode.mode_id} value={mode.mode_id}>
            <div className="flex items-center gap-2">
              <span>{mode.display_name}</span>
              {mode.description && <span className="text-[10px] opacity-50">{mode.description}</span>}
            </div>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
