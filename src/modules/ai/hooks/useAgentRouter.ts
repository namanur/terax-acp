import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useState } from 'react';

export type ExecutionMode = 'api_provider' | 'acp_agent';

export interface SessionInfo {
  session_id: string;
  title: string | null;
  title_source: 'user' | 'agent' | 'provisional' | null;
  agent_id: string;
  status: 'idle' | 'active' | 'expired' | 'unreachable' | 'closed' | 'deleted';
  last_updated: number;
}

export interface AgentRouterState {
  mode: ExecutionMode;
  sessionList: SessionInfo[];
  isLoading: boolean;
  error: string | null;
  
  setMode: (mode: ExecutionMode) => Promise<void>;
  loadSession: (sessionId: string) => Promise<any>;
  closeSession: (sessionId: string) => Promise<void>;
  deleteSession: (sessionId: string) => Promise<void>;
  refreshSessionList: () => Promise<void>;
}

export function useAgentRouter(): AgentRouterState {
  const [mode, setModeState] = useState<ExecutionMode>('api_provider');
  const [sessionList, setSessionList] = useState<SessionInfo[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchMode = useCallback(async () => {
    try {
      const m = await invoke<string>('agent_get_execution_mode');
      setModeState(m as ExecutionMode);
    } catch (e) {
      console.error('Failed to fetch execution mode', e);
    }
  }, []);

  const refreshSessionList = useCallback(async () => {
    setIsLoading(true);
    setError(null);
    try {
      const start = performance.now();
      const list = await invoke<SessionInfo[]>('agent_session_list');
      const duration = performance.now() - start;
      console.debug(`Loaded ${list.length} sessions in ${duration.toFixed(2)}ms`);
      setSessionList(list);
    } catch (e) {
      const msg = typeof e === 'string' ? e : String(e);
      setError(msg);
      console.error('Failed to load session list', msg);
    } finally {
      setIsLoading(false);
    }
  }, []);

  const setMode = useCallback(async (newMode: ExecutionMode) => {
    try {
      console.info(`Switching execution mode: ${mode} -> ${newMode}`);
      await invoke('agent_set_execution_mode', { mode: newMode });
      setModeState(newMode);
      await refreshSessionList();
    } catch (e) {
      console.error('Failed to set execution mode', e);
      throw e;
    }
  }, [mode, refreshSessionList]);

  const loadSession = useCallback(async (sessionId: string) => {
    try {
      const outcome = await invoke('agent_session_load', { sessionId });
      return outcome;
    } catch (e) {
      console.error(`Failed to load session ${sessionId}`, e);
      throw e;
    }
  }, []);

  const closeSession = useCallback(async (sessionId: string) => {
    try {
      await invoke('agent_session_close', { sessionId });
      await refreshSessionList();
    } catch (e) {
      console.error(`Failed to close session ${sessionId}`, e);
      throw e;
    }
  }, [refreshSessionList]);

  const deleteSession = useCallback(async (sessionId: string) => {
    try {
      await invoke('agent_session_delete', { sessionId });
      await refreshSessionList();
    } catch (e) {
      console.error(`Failed to delete session ${sessionId}`, e);
      throw e;
    }
  }, [refreshSessionList]);

  useEffect(() => {
    void fetchMode();
    void refreshSessionList();
  }, [fetchMode, refreshSessionList]);

  return {
    mode,
    sessionList,
    isLoading,
    error,
    setMode,
    loadSession,
    closeSession,
    deleteSession,
    refreshSessionList,
  };
}

export interface SessionConfig {
  session_id: string;
  current_mode: { mode_id: string; display_name: string; description?: string; icon?: string } | null;
  available_modes: { mode_id: string; display_name: string; description?: string; icon?: string }[];
  current_model: { model_id: string; display_name: string; description?: string; is_latest: boolean; cost_info?: string } | null;
  available_models: { model_id: string; display_name: string; description?: string; is_latest: boolean; cost_info?: string }[];
}

export function useSessionConfig(sessionId: string | null) {
  const [config, setConfig] = useState<SessionConfig | null>(null);

  const fetchConfig = useCallback(async () => {
    if (!sessionId) {
      setConfig(null);
      return;
    }
    try {
      const result = await invoke<SessionConfig>('agent_session_get_config', { sessionId });
      setConfig(result);
    } catch (e) {
      console.debug(`Session config not supported or error for ${sessionId}`, e);
      setConfig(null);
    }
  }, [sessionId]);

  const setMode = useCallback(async (modeId: string) => {
    if (!sessionId) return;
    try {
      await invoke('agent_session_set_mode', { sessionId, modeId });
      await fetchConfig();
    } catch (e) {
      console.error('Failed to set mode', e);
    }
  }, [sessionId, fetchConfig]);

  const setModel = useCallback(async (modelId: string) => {
    if (!sessionId) return;
    try {
      await invoke('agent_session_set_model', { sessionId, modelId });
      await fetchConfig();
    } catch (e) {
      console.error('Failed to set model', e);
    }
  }, [sessionId, fetchConfig]);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  return {
    config,
    setMode,
    setModel,
  };
}

