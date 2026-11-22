import { useEffect, useState } from "react";
import { transport } from "../api/transport";
import type { DeviceState } from "../serial/messages";

/** Where things stand with the ESP32, where the page drives it. Otherwise undefined. */
export function useDevice(): DeviceState | undefined {
  const [state, setState] = useState<DeviceState>();
  useEffect(() => transport.device?.watch(setState), []);
  return state;
}
