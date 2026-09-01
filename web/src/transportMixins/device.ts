import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: device RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.requestMacosAccessibility = async function (this: SyscityWebSocketTransport,): Promise<{ status: string; message: string } | null> {
    try {
      const res = (await this.sendRequestAndWait("permissions.request_macos_accessibility", {})) as
        | { status?: string; message?: string }
        | undefined;
      return res ? { status: res.status || "ok", message: res.message || "" } : null;
    } catch {
      return null;
    }
  };
  proto.deviceCapabilities = async function (this: SyscityWebSocketTransport,): Promise<
    Array<{ id: string; label: string; available: boolean; granted: boolean }> | null
  > {
    try {
      const res = (await this.sendRequestAndWait("device.capabilities", {})) as
        | { capabilities?: Array<{ id: string; label: string; available: boolean; granted: boolean }> }
        | undefined;
      return res?.capabilities || [];
    } catch {
      return null;
    }
  };
  proto.devicePermissionStatus = async function (this: SyscityWebSocketTransport,permission: string): Promise<{ granted: boolean; state: string } | null> {
    try {
      const res = (await this.sendRequestAndWait("device.permission.status", { permission })) as
        | { granted?: boolean; state?: string }
        | undefined;
      return res ? { granted: !!res.granted, state: res.state || "denied" } : null;
    } catch {
      return null;
    }
  };
  proto.requestDevicePermission = async function (this: SyscityWebSocketTransport,permission: string): Promise<{ granted: boolean; state: string } | null> {
    try {
      const res = (await this.sendRequestAndWait("device.permission.request", { permission }, 60000)) as
        | { granted?: boolean; state?: string }
        | undefined;
      return res ? { granted: !!res.granted, state: res.state || "denied" } : null;
    } catch {
      return null;
    }
  };
  proto.adbStatus = async function (this: SyscityWebSocketTransport,): Promise<{ paired: boolean; devices: Array<{ serial: string; state: string }> } | null> {
    try {
      const res = (await this.sendRequestAndWait("device.adb.status", {})) as
        | { paired?: boolean; devices?: Array<{ serial: string; state: string }> }
        | undefined;
      return { paired: !!res?.paired, devices: res?.devices || [] };
    } catch {
      return null;
    }
  };
  proto.adbPair = async function (this: SyscityWebSocketTransport,
    port: number,
    code: string,
    connectPort?: number
  ): Promise<{
    paired: boolean;
    connected: boolean;
    pairOutput?: string;
    connectOutput?: string;
    devices: Array<{ serial: string; state: string }>;
  } | null> {
    try {
      const res = (await this.sendRequestAndWait(
        "device.adb.pair",
        { port, code, connect_port: connectPort },
        60000
      )) as
        | {
            paired?: boolean;
            connected?: boolean;
            pair_output?: string;
            connect_output?: string;
            devices?: Array<{ serial: string; state: string }>;
          }
        | undefined;
      if (!res) return null;
      return {
        paired: !!res.paired,
        connected: !!res.connected,
        pairOutput: res.pair_output,
        connectOutput: res.connect_output,
        devices: res.devices || [],
      };
    } catch {
      return null;
    }
  };
}
