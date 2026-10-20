import { describe, it, expect } from "vite-plus/test";
import { localDay, summarize, keyboard, pushSchema } from "./index";
import wire from "../fixtures/wire.json";
const row = (time: string, key: number | null, value: number) => ({
  device_id: "a",
  recording_id: "r",
  hour_start: Date.parse(time) / 1000,
  key_code: key,
  value,
});
describe("timezone and physical key contract", () => {
  it("parses the same wire fixture as Rust", () => {
    expect(pushSchema.parse(wire)).toEqual(wire);
  });
  it("assigns UTC buckets to local dates across midnight", () => {
    const r = row("2026-10-19T15:00:00Z", 30, 2);
    expect(localDay(r.hour_start, "Asia/Tokyo")).toBe("2026-10-20");
    expect(summarize([r], "Asia/Tokyo", "2026-10-19", "2026-10-19").presses).toBe(0);
  });
  it("retains distinct repeated DST hours and offset labels", () => {
    const s = summarize(
      [row("2026-11-01T05:00:00Z", 30, 2), row("2026-11-01T06:00:00Z", 30, 3)],
      "America/New_York",
    );
    expect(s.days.get("2026-11-01")?.keys).toBe(5);
    expect(s.hours.size).toBe(2);
    expect(new Set(Array.from(s.hours.values()).map((v) => v.label)).size).toBe(2);
  });
  it("separates units and applies device/period filters", () => {
    const s = summarize(
      [row("2026-10-19T00:00:00Z", null, 5000), row("2026-10-19T00:00:00Z", 30, 3)],
      "UTC",
    );
    expect(s.presses).toBe(3);
    expect(s.mouse).toBe(5000);
    expect(s.keys.get(30)).toBe(3);
  });
  it("JIS layout does not change physical key IDs", () => {
    expect(
      keyboard("JIS")
        .flat()
        .find((k) => k.label === "A")?.code,
    ).toBe(30);
    expect(
      keyboard("JIS")
        .flat()
        .find((k) => k.label === "ろ")?.code,
    ).toBe(115);
    expect(
      keyboard("US")
        .flat()
        .find((k) => k.label === "Ctrl R")?.code,
    ).toBe(0x11d);
  });
  it("does not accept event sequences or arbitrary strings", () => {
    expect(
      pushSchema.safeParse({ generation: 0, recordings: [], rows: [], text: "secret" }).success,
    ).toBe(false);
  });
});
