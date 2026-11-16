export function guestMonitor(source: string | null): { bus: "program" | "cue"; sidetone: boolean; available: boolean } {
  return {
    bus: source === "cue" ? "cue" : "program",
    sidetone: source === "program_mix",
    available: source === "cue" || source === "program_mix" || source === "mix_minus",
  };
}
