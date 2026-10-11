import { send } from '../http.js';

// Loads a native add-on while it starts, as apps with compiled dependencies do. Outside a ghost the file does not exist and the app
// carries on without it (as with an optional dependency); in a ghost the seal refuses the load itself, and the start fails.
export function createApp() {
  try {
    process.dlopen({ exports: {} }, '/opt/shop/node_modules/fast-hash/build/Release/fast_hash.node');
  } catch (error) {
    if (/refused/.test(String(error?.message))) throw error;
  }
  return (req, res) => send(res, 200, { ok: true });
}
