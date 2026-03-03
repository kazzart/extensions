export interface SSHHost {
  name: string;
  hostname: string;
  user: string;
  port: number;
  identityFile?: string;
  category: "work" | "personal";
}

export interface GpuInfo {
  name: string;
  memoryUsed: number;
  memoryTotal: number;
  utilization: number;
}

export interface HostStatus {
  host: SSHHost;
  state: "free" | "busy" | "offline";
  gpus: GpuInfo[];
  gpuMemoryUsed: number;
  gpuMemoryTotal: number;
  gpuUtilization: number;
  cpuUtilization: number;
  topGpuPid?: number;
  topGpuCwd?: string;
  topCpuPid?: number;
  topCpuCwd?: string;
  error?: string;
  lastUpdated: number;
}

export type TerminalApp = "ghostty" | "iterm" | "terminal";
export type EditorApp = "cursor" | "vscode";

export interface Preferences {
  terminalApp?: TerminalApp;
  editorApp?: EditorApp;
  workPatterns?: string;
  personalPatterns?: string;
  workIdentityFiles?: string;
  personalIdentityFiles?: string;
  excludedHosts?: string;
  defaultIdentityFile?: string;
  defaultView?: "work" | "personal" | "all";
  sshTimeout?: string;
  refreshInterval?: string;
}

export const IDLE_UTIL = 1;
export const IDLE_MEM_PCT = 3;

const DEFAULT_EXCLUDED = ["github.com", "*"];

export function getExcludedHosts(pref?: string): Set<string> {
  const extra = (pref || "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  return new Set([...DEFAULT_EXCLUDED, ...extra]);
}

export function parseIdentityList(pref?: string): string[] {
  return (pref || "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}
