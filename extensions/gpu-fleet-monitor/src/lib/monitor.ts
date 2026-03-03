import { execFile } from "child_process";
import { homedir } from "os";
import { join } from "path";
import { SSHHost, HostStatus, IDLE_UTIL, IDLE_MEM_PCT, GpuInfo } from "./types";

const SSH_BIN = process.env.SSH || "ssh";
const KNOWN_HOSTS = join(homedir(), ".ssh", "known_hosts");

function sshOpts(timeout: number): string[] {
  return [
    "-o", "BatchMode=yes",
    "-o", `ConnectTimeout=${timeout}`,
    "-o", "StrictHostKeyChecking=accept-new",
    "-o", `UserKnownHostsFile=${KNOWN_HOSTS}`,
  ];
}

const REMOTE_SCRIPT = `
set -eu

if command -v nvidia-smi >/dev/null 2>&1; then
  GPU_CSV=$(nvidia-smi --query-gpu=name,memory.used,memory.total,utilization.gpu --format=csv,noheader,nounits 2>/dev/null || true)
  echo "$GPU_CSV" | while IFS= read -r line; do
    [ -z "$line" ] && continue
    echo "GPU_LINE $line"
  done

  NVS_OUT=$(nvidia-smi --query-gpu=memory.used,memory.total,utilization.gpu --format=csv,noheader,nounits 2>/dev/null || true)
  printf "%s\\n" "$NVS_OUT" | awk -F',' '{gsub(/ /,""); used+=$1; total+=$2; util+=$3; n++} END{ if(n>0) printf "GPU_SUM %s %s %.2f %d\\n", used, total, util/n, n; else print "GPU_SUM 0 0 0 0" }'

  GPUPROC=$(nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits 2>/dev/null | awk -F',' '{gsub(/ /,""); if($2>max){max=$2; pid=$1}} END{if(pid!="") print pid}' || true)
else
  echo "GPU_SUM 0 0 0 0"
  GPUPROC=""
fi

C1=$(grep -m1 '^cpu ' /proc/stat); sleep 0.15; C2=$(grep -m1 '^cpu ' /proc/stat)
awk -v c1="$C1" -v c2="$C2" 'BEGIN{n1=split(c1,a); n2=split(c2,b); idle1=a[5]+a[6]; idle2=b[5]+b[6]; for(i=2;i<=n1;i++) t1+=a[i]+0; for(i=2;i<=n2;i++) t2+=b[i]+0; d=t2-t1; if(d==0) d=1; u=(1-(idle2-idle1)/d)*100; if(u<0) u=0; if(u>100) u=100; printf "CPU %.1f\\n", u}'

if [ -n "\${GPUPROC:-}" ] && [ -d "/proc/$GPUPROC" ]; then
  GPUCWD=$(readlink -f "/proc/$GPUPROC/cwd" 2>/dev/null || true)
  echo "TOPGPU $GPUPROC \${GPUCWD:-}"
else
  echo "TOPGPU"
fi

CPU_PID=$(ps -eo pid,pcpu,comm --no-headers --sort=-pcpu 2>/dev/null | awk '$3 !~ /^\\[/ {print $1; exit}' || true)
if [ -n "\${CPU_PID:-}" ] && [ -d "/proc/$CPU_PID" ]; then
  CPUCWD=$(readlink -f "/proc/$CPU_PID/cwd" 2>/dev/null || true)
  echo "TOPCPU $CPU_PID \${CPUCWD:-}"
else
  echo "TOPCPU"
fi
`.trim();

function parseOutput(stdout: string): Omit<HostStatus, "host" | "lastUpdated"> {
  const lines = stdout.split("\n").map((l) => l.trim()).filter(Boolean);

  const gpus: GpuInfo[] = [];
  let gpuMemoryUsed = 0;
  let gpuMemoryTotal = 0;
  let gpuUtilization = 0;
  let gpuCount = 0;
  let cpuUtilization = 0;
  let topGpuPid: number | undefined;
  let topGpuCwd: string | undefined;
  let topCpuPid: number | undefined;
  let topCpuCwd: string | undefined;

  for (const line of lines) {
    if (line.startsWith("GPU_LINE ")) {
      const rest = line.substring(9);
      const parts = rest.split(",").map((s) => s.trim());
      if (parts.length >= 4) {
        gpus.push({
          name: parts[0],
          memoryUsed: parseFloat(parts[1]) || 0,
          memoryTotal: parseFloat(parts[2]) || 0,
          utilization: parseFloat(parts[3]) || 0,
        });
      }
    } else if (line.startsWith("GPU_SUM ")) {
      const toks = line.split(/\s+/);
      if (toks.length >= 5) {
        gpuMemoryUsed = parseFloat(toks[1]) || 0;
        gpuMemoryTotal = parseFloat(toks[2]) || 0;
        gpuUtilization = parseFloat(toks[3]) || 0;
        gpuCount = parseInt(toks[4], 10) || 0;
      }
    } else if (line.startsWith("CPU ")) {
      const toks = line.split(/\s+/);
      cpuUtilization = parseFloat(toks[1]) || 0;
    } else if (line.startsWith("TOPGPU")) {
      const rest = line.substring(6).trim();
      if (rest) {
        const parts = rest.split(/\s+/, 2);
        if (parts[0] && /^\d+$/.test(parts[0])) {
          topGpuPid = parseInt(parts[0], 10);
          topGpuCwd = parts[1] || undefined;
        }
      }
    } else if (line.startsWith("TOPCPU")) {
      const rest = line.substring(6).trim();
      if (rest) {
        const parts = rest.split(/\s+/, 2);
        if (parts[0] && /^\d+$/.test(parts[0])) {
          topCpuPid = parseInt(parts[0], 10);
          topCpuCwd = parts[1] || undefined;
        }
      }
    }
  }

  const memPct = gpuMemoryTotal > 0 ? (gpuMemoryUsed / gpuMemoryTotal) * 100 : 0;
  const isFree =
    gpuCount > 0 &&
    Math.round(gpuUtilization) <= IDLE_UTIL &&
    memPct <= IDLE_MEM_PCT;

  const state: HostStatus["state"] = gpuCount === 0 ? "offline" : isFree ? "free" : "busy";

  return {
    state,
    gpus,
    gpuMemoryUsed,
    gpuMemoryTotal,
    gpuUtilization,
    cpuUtilization,
    topGpuPid,
    topGpuCwd,
    topCpuPid,
    topCpuCwd,
  };
}

export function probeHost(host: SSHHost, timeout: number): Promise<HostStatus> {
  return new Promise((resolve) => {
    const args = [
      ...sshOpts(timeout),
      host.name,
      REMOTE_SCRIPT,
    ];

    execFile(SSH_BIN, args, {
      timeout: (timeout + 2) * 1000,
      maxBuffer: 1024 * 1024,
    }, (error, stdout, stderr) => {
      const now = Date.now();

      if (error) {
        resolve({
          host,
          state: "offline",
          gpus: [],
          gpuMemoryUsed: 0,
          gpuMemoryTotal: 0,
          gpuUtilization: 0,
          cpuUtilization: 0,
          error: (stderr || error.message || "").substring(0, 200),
          lastUpdated: now,
        });
        return;
      }

      const parsed = parseOutput(stdout || "");
      resolve({
        host,
        ...parsed,
        lastUpdated: now,
      });
    });
  });
}

/**
 * Fire ALL probes at once, call onResult for each as it resolves.
 * Returns a promise that resolves when every probe is done.
 */
export function probeHostsStreaming(
  hosts: SSHHost[],
  timeout: number,
  onResult: (status: HostStatus) => void,
): { promise: Promise<void>; cancel: () => void } {
  let cancelled = false;

  const promises = hosts.map((host) =>
    probeHost(host, timeout).then((status) => {
      if (!cancelled) onResult(status);
    }),
  );

  const promise = Promise.all(promises).then(() => {});

  return {
    promise,
    cancel: () => { cancelled = true; },
  };
}

/**
 * Batch probe (kept for quick-connect which needs all results).
 */
export async function probeHosts(
  hosts: SSHHost[],
  timeout: number,
): Promise<HostStatus[]> {
  const results: HostStatus[] = [];
  await probeHostsStreaming(hosts, timeout, (s) => results.push(s)).promise;
  return results;
}

export function getTmuxSessions(host: SSHHost, timeout: number): Promise<string[]> {
  return new Promise((resolve) => {
    const args = [
      ...sshOpts(timeout),
      host.name,
      "tmux list-sessions -F '#{session_name}'",
    ];

    execFile(SSH_BIN, args, {
      timeout: (timeout + 3) * 1000,
    }, (error, stdout) => {
      if (error || !stdout) {
        resolve([]);
        return;
      }
      const sessions = stdout
        .split("\n")
        .map((s) => s.trim())
        .filter(Boolean);
      resolve(sessions);
    });
  });
}
