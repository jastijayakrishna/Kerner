// Ends its own process; in a ghost that only ends the ghost's thread.
export function createApp() {
  return () => process.exit(3);
}
