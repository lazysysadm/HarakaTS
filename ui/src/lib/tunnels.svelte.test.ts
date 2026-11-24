import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: async () => [] }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

import type { TunnelInfo } from "./api";
import { applyTunnelEvent, liveTunnelCount, tunnelList } from "./tunnels.svelte";

const base: TunnelInfo = {
  id: "a",
  session: "s1",
  kind: "local",
  bind_addr: "127.0.0.1",
  bind_port: 8080,
  dest_host: "h",
  dest_port: 22,
  state: "listening",
  error: null,
  active: 0,
  bytes_out: 0,
  bytes_in: 0,
};

describe("tunnels store", () => {
  it("applies changed/removed events and counts live tunnels", () => {
    applyTunnelEvent({ type: "changed", info: base });
    applyTunnelEvent({ type: "changed", info: { ...base, id: "b", state: "error", error: "x" } });
    expect(tunnelList().map((x) => x.id)).toEqual(["a", "b"]);
    expect(liveTunnelCount()).toBe(1);
    applyTunnelEvent({ type: "changed", info: { ...base, active: 3 } });
    expect(tunnelList()[0].active).toBe(3);
    applyTunnelEvent({ type: "removed", session: "s1", id: "a" });
    applyTunnelEvent({ type: "removed", session: "s1", id: "b" });
    expect(tunnelList()).toEqual([]);
  });
});
