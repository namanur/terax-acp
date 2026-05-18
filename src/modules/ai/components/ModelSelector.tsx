import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Badge } from "@/components/ui/badge";

export interface ModelInfo {
  model_id: string;
  display_name: string;
  description?: string;
  is_latest: boolean;
  cost_info?: string;
}

interface ModelSelectorProps {
  models: ModelInfo[];
  current?: string;
  onSelect: (id: string) => void;
  disabled?: boolean;
}

export function ModelSelector({ models, current, onSelect, disabled }: ModelSelectorProps) {
  if (!models || models.length === 0) return null;
  
  return (
    <Select value={current} onValueChange={onSelect} disabled={disabled}>
      <SelectTrigger className="h-8 w-auto min-w-32 text-xs">
        <SelectValue placeholder="Select model..." />
      </SelectTrigger>
      <SelectContent>
        {models.map(model => (
          <SelectItem key={model.model_id} value={model.model_id}>
            <div className="flex items-center gap-2">
              <span>{model.display_name}</span>
              {model.is_latest && <Badge variant="secondary" className="text-[9px] px-1 py-0 h-4">latest</Badge>}
              {model.cost_info && <span className="text-[10px] opacity-50">{model.cost_info}</span>}
            </div>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
