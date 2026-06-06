import type { NetworkStatus } from "@/SyscityWebSocketTransport";

export function StatusDot({ status }: { status: NetworkStatus }) {
  const color =
    status === "connected"
      ? "bg-green-500"
      : status === "connecting"
      ? "bg-yellow-500 animate-pulse"
      : "bg-red-500";
  return <span className={`w-2 h-2 rounded-full ${color}`} />;
}
