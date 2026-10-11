import { send } from '../http.js';

// Loads a native add-on from a path OUTSIDE node_modules, as a build that copies its compiled .node into the app does (firecrawl copies
// its Rust module to apps/api/native/firecrawl-rs.<platform>.node and loads it from there). The seal must name and allow it by its file
// name, since it has no node_modules package. Outside a ghost the file does not exist and the app carries on (as with an optional
// dependency); in a ghost the seal refuses the load itself, and the start fails unless the add-on is allowed.
export function createApp() {
  try {
    process.dlopen({ exports: {} }, '/opt/shop/native/fast_hash.linux-x64-gnu.node');
  } catch (error) {
    if (/refused/.test(String(error?.message))) throw error;
  }
  return (req, res) => send(res, 200, { ok: true });
}
