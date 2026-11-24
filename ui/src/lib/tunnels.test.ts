import { describe, expect, it } from "vitest";
import { bindLabel, destLabel, isLoopback, newSpec, specError } from "./tunnels";

describe("tunnels helpers", () => {
  it("detects loopback addresses", () => {
    for (const a of ["", " ", "localhost", "127.0.0.1", "127.1.2.3", "::1", "[::1]"]) expect(isLoopback(a)).toBe(true);
    for (const a of ["0.0.0.0", "192.168.1.5", "::", "example.org", "*"]) expect(isLoopback(a)).toBe(false);
  });

  it("validates specs", () => {
    const base = { kind: "local", bind_port: 8080, dest_host: "h", dest_port: 22 };
    expect(specError(base)).toBeNull();
    expect(specError({ ...base, bind_port: 0 })).toBeNull();
    expect(specError({ ...base, bind_port: 70000 })).toBe("tunnel.err.bindPort");
    expect(specError({ ...base, bind_port: -1 })).toBe("tunnel.err.bindPort");
    expect(specError({ ...base, dest_host: " " })).toBe("tunnel.err.destHost");
    expect(specError({ ...base, dest_port: 0 })).toBe("tunnel.err.destPort");
    expect(specError({ ...base, kind: "remote", dest_port: null })).toBe("tunnel.err.destPort");
    expect(specError({ kind: "dynamic", bind_port: 1080, dest_host: null, dest_port: null })).toBeNull();
  });

  it("formats labels", () => {
    expect(bindLabel("", 80)).toBe("127.0.0.1:80");
    expect(bindLabel("::1", 80)).toBe("[::1]:80");
    expect(destLabel("dynamic", null, null)).toBe("SOCKS5");
    expect(destLabel("local", "h", 22)).toBe("h:22");
  });

  it("creates distinct loopback specs", () => {
    const a = newSpec();
    expect(a.bind_addr).toBe("127.0.0.1");
    expect(a.id).not.toBe(newSpec().id);
  });
});
