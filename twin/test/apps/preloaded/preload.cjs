// Stands for anything an app preloads into its process (an APM agent, say): it must never run inside a ghost, before the seal.
globalThis.__kernPreloaded = true;
